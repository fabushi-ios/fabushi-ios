use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};
use thiserror::Error;

const DEFAULT_MAX_CATALOG_BYTES: usize = 256 * 1024;
const MAX_CATALOG_ID_CHARS: usize = 256;
const MAX_ARCH_CHARS: usize = 64;

/// A bounded, side-effect-free parser for explicitly configured worker
/// provider discovery output. It never turns an advertised capability into
/// authorization or a launch decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CloudWorkerCatalogParser {
    max_bytes: usize,
}

impl Default for CloudWorkerCatalogParser {
    fn default() -> Self {
        Self {
            max_bytes: DEFAULT_MAX_CATALOG_BYTES,
        }
    }
}

impl CloudWorkerCatalogParser {
    pub fn with_max_bytes(max_bytes: usize) -> Result<Self, CloudWorkerCatalogError> {
        if max_bytes == 0 {
            return Err(CloudWorkerCatalogError::InvalidLimit);
        }
        Ok(Self { max_bytes })
    }

    pub fn parse(&self, output: &[u8]) -> Result<CloudWorkerCatalog, CloudWorkerCatalogError> {
        if output.len() > self.max_bytes {
            return Err(CloudWorkerCatalogError::OutputTooLarge {
                bytes: output.len(),
                max_bytes: self.max_bytes,
            });
        }
        let catalog: CloudWorkerCatalog =
            serde_json::from_slice(output).map_err(CloudWorkerCatalogError::MalformedJson)?;
        catalog.validate()
    }
}

/// Exact identity of the executable that produced a provider catalog.
///
/// The cache key is deliberately opaque to this contract: the provider/binary
/// owner supplies a stable digest or equivalent identity, and this module does
/// not infer identity from a mutable path or advertised catalog contents.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CloudWorkerBinaryIdentity(String);

impl CloudWorkerBinaryIdentity {
    pub fn new(value: impl Into<String>) -> Result<Self, CloudWorkerCatalogError> {
        let value = value.into();
        if !valid_text(&value, MAX_CATALOG_ID_CHARS) {
            return Err(CloudWorkerCatalogError::InvalidBinaryIdentity);
        }
        Ok(Self(value))
    }
}

/// Side-effect-free catalog cache owned by the provider/binary boundary.
///
/// A successful parse is cached only under the exact binary identity. A failed
/// load always invalidates the identity first, so stale capability data cannot
/// survive a failed refresh. This type performs no discovery, authorization,
/// process launch, or provider mutation.
#[derive(Debug, Default)]
pub struct CloudWorkerCatalogCache {
    entries: HashMap<CloudWorkerBinaryIdentity, CloudWorkerCatalog>,
}

impl CloudWorkerCatalogCache {
    pub fn get(&self, identity: &CloudWorkerBinaryIdentity) -> Option<&CloudWorkerCatalog> {
        self.entries.get(identity)
    }

    pub fn invalidate(&mut self, identity: &CloudWorkerBinaryIdentity) {
        self.entries.remove(identity);
    }

    pub fn resolve<F>(
        &mut self,
        identity: CloudWorkerBinaryIdentity,
        load: F,
    ) -> Result<CloudWorkerCatalog, CloudWorkerCatalogError>
    where
        F: FnOnce() -> Result<CloudWorkerCatalog, CloudWorkerCatalogError>,
    {
        if let Some(catalog) = self.entries.get(&identity) {
            return Ok(catalog.clone());
        }

        self.refresh(identity, load)
    }

    /// Refresh an identity even when a previous catalog is cached.
    ///
    /// The old entry is removed before loading so an error cannot leave stale
    /// capability data addressable under the identity.
    pub fn refresh<F>(
        &mut self,
        identity: CloudWorkerBinaryIdentity,
        load: F,
    ) -> Result<CloudWorkerCatalog, CloudWorkerCatalogError>
    where
        F: FnOnce() -> Result<CloudWorkerCatalog, CloudWorkerCatalogError>,
    {
        self.entries.remove(&identity);

        match load().and_then(CloudWorkerCatalog::validate) {
            Ok(catalog) => {
                self.entries.insert(identity, catalog.clone());
                Ok(catalog)
            }
            Err(error) => {
                self.entries.remove(&identity);
                Err(error)
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CloudWorkerCatalog {
    pub providers: Vec<CloudWorkerProvider>,
}

impl CloudWorkerCatalog {
    pub fn select_machine(
        &self,
        provider_id: &str,
        profile_id: &str,
        requirement: &CloudWorkerMachineRequirement,
    ) -> Result<CloudWorkerMachine, CloudWorkerCatalogError> {
        let provider = self
            .providers
            .iter()
            .find(|provider| provider.id == provider_id)
            .ok_or_else(|| CloudWorkerCatalogError::UnknownProvider(provider_id.into()))?;
        let profile = provider
            .profiles
            .iter()
            .find(|profile| profile.id == profile_id)
            .ok_or_else(|| CloudWorkerCatalogError::UnknownProfile {
                provider: provider_id.into(),
                profile: profile_id.into(),
            })?;
        profile
            .select_machine(requirement)
            .cloned()
            .ok_or_else(|| CloudWorkerCatalogError::NoCompatibleMachine {
                provider: provider_id.into(),
                profile: profile_id.into(),
            })
    }

    pub fn validate(mut self) -> Result<Self, CloudWorkerCatalogError> {
        if self.providers.is_empty() {
            return Err(CloudWorkerCatalogError::EmptyCatalog);
        }

        let mut provider_ids = HashSet::new();
        for provider in &mut self.providers {
            provider.normalize_and_validate()?;
            if !provider_ids.insert(provider.id.clone()) {
                return Err(CloudWorkerCatalogError::DuplicateProvider(provider.id.clone()));
            }
        }
        self.providers.sort_by(|left, right| left.id.cmp(&right.id));
        Ok(self)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CloudWorkerProvider {
    pub id: String,
    pub profiles: Vec<CloudWorkerProfile>,
}

impl CloudWorkerProvider {
    fn normalize_and_validate(&mut self) -> Result<(), CloudWorkerCatalogError> {
        normalize_id(&mut self.id).map_err(|_| CloudWorkerCatalogError::InvalidProviderId)?;
        if self.profiles.is_empty() {
            return Err(CloudWorkerCatalogError::EmptyProviderProfiles(self.id.clone()));
        }

        let mut profile_ids = HashSet::new();
        for profile in &mut self.profiles {
            profile.normalize_and_validate()?;
            if !profile_ids.insert(profile.id.clone()) {
                return Err(CloudWorkerCatalogError::DuplicateProfile {
                    provider: self.id.clone(),
                    profile: profile.id.clone(),
                });
            }
        }
        self.profiles.sort_by(|left, right| left.id.cmp(&right.id));
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CloudWorkerProfile {
    pub id: String,
    pub machines: Vec<CloudWorkerMachine>,
}

impl CloudWorkerProfile {
    pub fn select_machine(
        &self,
        requirement: &CloudWorkerMachineRequirement,
    ) -> Option<&CloudWorkerMachine> {
        self.machines
            .iter()
            .find(|machine| machine.supports(requirement))
    }

    fn normalize_and_validate(&mut self) -> Result<(), CloudWorkerCatalogError> {
        normalize_id(&mut self.id).map_err(|_| CloudWorkerCatalogError::InvalidProfileId)?;
        if self.machines.is_empty() {
            return Err(CloudWorkerCatalogError::EmptyProfileMachines(self.id.clone()));
        }
        for machine in &self.machines {
            machine.validate()?;
        }
        self.machines.sort_by(|left, right| {
            left.os
                .cmp(&right.os)
                .then(left.arch.cmp(&right.arch))
                .then(left.resources.cmp(&right.resources))
        });
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CloudWorkerMachine {
    pub os: CloudWorkerOs,
    pub arch: String,
    pub resources: CloudWorkerResourceLimits,
}

impl CloudWorkerMachine {
    pub fn supports(&self, requirement: &CloudWorkerMachineRequirement) -> bool {
        self.os == requirement.os
            && self.arch == requirement.arch
            && self.resources.cpu_cores >= requirement.min_cpu_cores
            && self.resources.memory_bytes >= requirement.min_memory_bytes
    }

    fn validate(&self) -> Result<(), CloudWorkerCatalogError> {
        if !valid_text(&self.arch, MAX_ARCH_CHARS) {
            return Err(CloudWorkerCatalogError::InvalidArchitecture);
        }
        if self.resources.cpu_cores == 0 || self.resources.memory_bytes == 0 {
            return Err(CloudWorkerCatalogError::InvalidResources);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum CloudWorkerOs {
    Linux,
    Macos,
    Windows,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct CloudWorkerResourceLimits {
    pub cpu_cores: u16,
    pub memory_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CloudWorkerMachineRequirement {
    pub os: CloudWorkerOs,
    pub arch: String,
    pub min_cpu_cores: u16,
    pub min_memory_bytes: u64,
}

#[derive(Debug, Error)]
pub enum CloudWorkerCatalogError {
    #[error("cloud-worker owner is unavailable")]
    OwnerUnavailable,
    #[error("provider catalog is not cached for this binary identity")]
    UnknownBinaryIdentity,
    #[error("catalog parser limit must be greater than zero")]
    InvalidLimit,
    #[error("binary identity is empty or contains surrounding whitespace")]
    InvalidBinaryIdentity,
    #[error("catalog output is {bytes} bytes, above the {max_bytes}-byte limit")]
    OutputTooLarge { bytes: usize, max_bytes: usize },
    #[error("catalog output is not valid JSON: {0}")]
    MalformedJson(#[source] serde_json::Error),
    #[error("catalog must contain at least one provider")]
    EmptyCatalog,
    #[error("provider id is empty or contains surrounding whitespace")]
    InvalidProviderId,
    #[error("provider {0} has no profiles")]
    EmptyProviderProfiles(String),
    #[error("provider id is duplicated: {0}")]
    DuplicateProvider(String),
    #[error("profile id is empty or contains surrounding whitespace")]
    InvalidProfileId,
    #[error("profile {0} has no machine choices")]
    EmptyProfileMachines(String),
    #[error("provider was not found in the cached catalog: {0}")]
    UnknownProvider(String),
    #[error("profile {profile} was not found under provider {provider}")]
    UnknownProfile { provider: String, profile: String },
    #[error("profile {profile} has no machine satisfying the requested resources under provider {provider}")]
    NoCompatibleMachine { provider: String, profile: String },
    #[error("profile {profile} is duplicated under provider {provider}")]
    DuplicateProfile { provider: String, profile: String },
    #[error("machine architecture is empty")]
    InvalidArchitecture,
    #[error("machine resources must declare non-zero CPU and memory")]
    InvalidResources,
}

fn normalize_id(value: &mut String) -> Result<(), ()> {
    let trimmed = value.trim();
    if !valid_text(value, MAX_CATALOG_ID_CHARS) || trimmed != value {
        return Err(());
    }
    Ok(())
}

fn valid_text(value: &str, max_chars: usize) -> bool {
    !value.trim().is_empty()
        && value.chars().count() <= max_chars
        && !value.chars().any(|character| character.is_control())
}

#[cfg(test)]
mod tests {
    use super::*;

    const VALID: &[u8] = br#"{
        "providers": [{
            "id": "local",
            "profiles": [{
                "id": "default",
                "machines": [{
                    "os": "Linux",
                    "arch": "x86_64",
                    "resources": {"cpu_cores": 2, "memory_bytes": 1073741824}
                }]
            }]
        }]
    }"#;

    #[test]
    fn parses_and_sorts_a_valid_catalog() {
        let catalog = CloudWorkerCatalogParser::default().parse(VALID).unwrap();
        assert_eq!(catalog.providers[0].id, "local");
        assert_eq!(catalog.providers[0].profiles[0].id, "default");
    }

    #[test]
    fn rejects_catalogs_over_the_bound() {
        let parser = CloudWorkerCatalogParser::with_max_bytes(1).unwrap();
        assert!(matches!(
            parser.parse(VALID),
            Err(CloudWorkerCatalogError::OutputTooLarge { .. })
        ));
    }

    #[test]
    fn rejects_duplicate_provider_ids() {
        let input = br#"{"providers":[
            {"id":"same","profiles":[{"id":"a","machines":[{"os":"Linux","arch":"x86_64","resources":{"cpu_cores":1,"memory_bytes":1}}]}]},
            {"id":"same","profiles":[{"id":"b","machines":[{"os":"Linux","arch":"x86_64","resources":{"cpu_cores":1,"memory_bytes":1}}]}]}
        ]}"#;
        assert!(matches!(
            CloudWorkerCatalogParser::default().parse(input),
            Err(CloudWorkerCatalogError::DuplicateProvider(id)) if id == "same"
        ));
    }

    #[test]
    fn selects_only_a_machine_meeting_all_declared_constraints() {
        let catalog = CloudWorkerCatalogParser::default().parse(VALID).unwrap();
        let profile = &catalog.providers[0].profiles[0];
        let requirement = CloudWorkerMachineRequirement {
            os: CloudWorkerOs::Linux,
            arch: "x86_64".into(),
            min_cpu_cores: 2,
            min_memory_bytes: 1024 * 1024 * 1024,
        };
        assert!(profile.select_machine(&requirement).is_some());
        let too_large = CloudWorkerMachineRequirement {
            min_memory_bytes: 2 * 1024 * 1024 * 1024,
            ..requirement.clone()
        };
        assert!(profile.select_machine(&too_large).is_none());
        assert_eq!(
            catalog
                .select_machine("local", "default", &requirement)
                .unwrap()
                .arch,
            "x86_64"
        );
        assert!(matches!(
            catalog.select_machine("missing", "default", &requirement),
            Err(CloudWorkerCatalogError::UnknownProvider(id)) if id == "missing"
        ));
    }

    #[test]
    fn caches_success_by_exact_binary_identity() {
        let mut cache = CloudWorkerCatalogCache::default();
        let identity = CloudWorkerBinaryIdentity::new("sha256:abc").unwrap();
        let first = cache
            .resolve(identity.clone(), || {
                CloudWorkerCatalogParser::default().parse(VALID)
            })
            .unwrap();
        let second = cache
            .resolve(identity.clone(), || {
                Err(CloudWorkerCatalogError::EmptyCatalog)
            })
            .unwrap();
        assert_eq!(first, second);
        assert!(cache.get(&identity).is_some());
    }

    #[test]
    fn failed_load_invalidates_stale_identity() {
        let mut cache = CloudWorkerCatalogCache::default();
        let identity = CloudWorkerBinaryIdentity::new("sha256:abc").unwrap();
        cache
            .resolve(identity.clone(), || {
                CloudWorkerCatalogParser::default().parse(VALID)
            })
            .unwrap();
        assert!(matches!(
            cache.refresh(identity.clone(), || {
                Err(CloudWorkerCatalogError::EmptyCatalog)
            }),
            Err(CloudWorkerCatalogError::EmptyCatalog)
        ));
        assert!(cache.get(&identity).is_none());
    }

    #[test]
    fn rejects_unvalidated_loader_catalog_before_caching() {
        let mut cache = CloudWorkerCatalogCache::default();
        let identity = CloudWorkerBinaryIdentity::new("sha256:unvalidated").unwrap();
        let invalid = CloudWorkerCatalog {
            providers: vec![CloudWorkerProvider {
                id: "provider\nwith-control".into(),
                profiles: vec![CloudWorkerProfile {
                    id: "default".into(),
                    machines: vec![CloudWorkerMachine {
                        os: CloudWorkerOs::Linux,
                        arch: "x86_64".into(),
                        resources: CloudWorkerResourceLimits {
                            cpu_cores: 1,
                            memory_bytes: 1,
                        },
                    }],
                }],
            }],
        };
        assert!(matches!(
            cache.resolve(identity.clone(), || Ok(invalid)),
            Err(CloudWorkerCatalogError::InvalidProviderId)
        ));
        assert!(cache.get(&identity).is_none());
    }

    #[test]
    fn rejects_controlled_or_oversized_catalog_identities() {
        assert!(matches!(
            CloudWorkerBinaryIdentity::new("binary\nidentity"),
            Err(CloudWorkerCatalogError::InvalidBinaryIdentity)
        ));
        let input = format!(
            r#"{{"providers":[{{"id":"{}","profiles":[{{"id":"default","machines":[{{"os":"Linux","arch":"x86_64","resources":{{"cpu_cores":1,"memory_bytes":1}}}}]}}]}}]}}"#,
            "p".repeat(257)
        );
        assert!(matches!(
            CloudWorkerCatalogParser::default().parse(input.as_bytes()),
            Err(CloudWorkerCatalogError::InvalidProviderId)
        ));
    }
}
