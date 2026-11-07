use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use thiserror::Error;

const MAX_ENV_NAME_CHARS: usize = 128;
const MAX_ENV_VALUE_CHARS: usize = 4096;
const MAX_ENV_ENTRIES: usize = 64;
const MAX_POLICY_DIGITS: usize = 8;
const MIN_REFRESH_AFTER_MS: u64 = 3_600_000;
const MIN_RETAIN_UNUSED_MS: u64 = 86_400_000;
const WORKER_WALLPAPER_WIDTH: u32 = 1024;
const WORKER_WALLPAPER_HEIGHT: u32 = 576;
const PNG_SIGNATURE: &[u8; 8] = b"\x89PNG\r\n\x1a\n";
const MAX_SUGGESTION_ID_CHARS: usize = 256;
const MAX_MACHINE_CLASS_CHARS: usize = 128;

/// A deterministic, side-effect-free representation of the setup environment
/// profile. Values are inputs to a future secret-egress/process owner; this
/// type never writes a file or mutates a child environment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CloudWorkerEnvironmentProfile {
    entries: BTreeMap<String, String>,
}

impl CloudWorkerEnvironmentProfile {
    pub fn new(
        entries: BTreeMap<String, String>,
    ) -> Result<Self, CloudWorkerAuxiliaryError> {
        if entries.len() > MAX_ENV_ENTRIES {
            return Err(CloudWorkerAuxiliaryError::EnvironmentEntryLimitReached);
        }
        for (name, value) in &entries {
            if !valid_environment_name(name) {
                return Err(CloudWorkerAuxiliaryError::InvalidEnvironmentName);
            }
            if value.chars().count() > MAX_ENV_VALUE_CHARS
                || value.chars().any(|character| character.is_control())
                || value.contains('`')
                || value.contains("$(")
            {
                return Err(CloudWorkerAuxiliaryError::UnsafeEnvironmentValue);
            }
        }
        Ok(Self { entries })
    }

    pub fn entries(&self) -> &BTreeMap<String, String> {
        &self.entries
    }

    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.entries.keys().map(String::as_str)
    }
}

/// Warm-image retention policy parsed from the provider configuration. It is
/// policy data only: allocation, capture, deletion and scheduling stay with
/// their existing owners.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CloudWorkerWarmImagePolicy {
    pub refresh_after_ms: u64,
    pub retain_unused_ms: u64,
    pub keep_previous: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CloudWorkerInstallMode {
    Bundle,
}

/// The pinned Crabbox profile suggestion projected for existing setup/UI
/// consumers. It is deliberately not a provider authorization or readiness
/// result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CloudWorkerProfileSuggestion {
    pub id: String,
    pub provider_display_id: String,
    pub provider: String,
    pub install: CloudWorkerInstallMode,
    pub target: super::CloudWorkerOs,
    pub machine_class: String,
    pub suspend_after: String,
    pub ttl: String,
    pub idle_timeout: String,
    pub warm_image: bool,
}

impl CloudWorkerProfileSuggestion {
    pub fn new(
        provider_display_id: impl Into<String>,
        machine_class: impl Into<String>,
    ) -> Result<Self, CloudWorkerAuxiliaryError> {
        let provider_display_id = provider_display_id.into();
        let machine_class = machine_class.into();
        if !valid_bounded_text(&provider_display_id, MAX_SUGGESTION_ID_CHARS)
            || !valid_bounded_text(&machine_class, MAX_MACHINE_CLASS_CHARS)
        {
            return Err(CloudWorkerAuxiliaryError::InvalidProfileSuggestion);
        }
        Ok(Self {
            id: "cloud".into(),
            provider_display_id,
            provider: "crabbox".into(),
            install: CloudWorkerInstallMode::Bundle,
            target: super::CloudWorkerOs::Linux,
            machine_class,
            suspend_after: "45m".into(),
            ttl: "4h".into(),
            idle_timeout: "45m".into(),
            warm_image: true,
        })
    }
}

impl CloudWorkerWarmImagePolicy {
    pub fn from_config(
        refresh_after: Option<&str>,
        retain_unused: Option<&str>,
        keep_previous: bool,
    ) -> Result<Self, CloudWorkerAuxiliaryError> {
        let refresh_after_ms = parse_duration(
            refresh_after.unwrap_or("24h"),
            MIN_REFRESH_AFTER_MS,
            "refreshAfter",
        )?;
        let retain_unused_ms = parse_duration(
            retain_unused.unwrap_or("14d"),
            MIN_RETAIN_UNUSED_MS,
            "retainUnused",
        )?;
        Ok(Self {
            refresh_after_ms,
            retain_unused_ms,
            keep_previous,
        })
    }
}

/// Validates the exact image shape accepted by the worker wallpaper owner.
/// File access, base64 encoding and platform setup remain outside this
/// contract.
pub fn validate_worker_wallpaper_png(bytes: &[u8]) -> Result<(), CloudWorkerAuxiliaryError> {
    if bytes.len() < 33
        || &bytes[..PNG_SIGNATURE.len()] != PNG_SIGNATURE
        || u32::from_be_bytes(bytes[8..12].try_into().expect("length checked")) != 13
        || &bytes[12..16] != b"IHDR"
    {
        return Err(CloudWorkerAuxiliaryError::InvalidWallpaperPng);
    }
    let width = u32::from_be_bytes(bytes[16..20].try_into().expect("length checked"));
    let height = u32::from_be_bytes(bytes[20..24].try_into().expect("length checked"));
    if width != WORKER_WALLPAPER_WIDTH || height != WORKER_WALLPAPER_HEIGHT {
        return Err(CloudWorkerAuxiliaryError::InvalidWallpaperDimensions {
            width,
            height,
        });
    }
    Ok(())
}

fn parse_duration(
    value: &str,
    minimum_ms: u64,
    field: &'static str,
) -> Result<u64, CloudWorkerAuxiliaryError> {
    let Some(unit) = value.chars().last() else {
        return Err(CloudWorkerAuxiliaryError::InvalidDuration { field });
    };
    let digits = &value[..value.len().saturating_sub(unit.len_utf8())];
    if digits.is_empty()
        || digits.len() > MAX_POLICY_DIGITS
        || !digits.bytes().all(|byte| byte.is_ascii_digit())
        || digits.starts_with('0')
    {
        return Err(CloudWorkerAuxiliaryError::InvalidDuration { field });
    }
    let multiplier = match unit {
        'm' => 60_000,
        'h' => 3_600_000,
        'd' => 86_400_000,
        _ => return Err(CloudWorkerAuxiliaryError::InvalidDuration { field }),
    };
    let amount = digits
        .parse::<u64>()
        .map_err(|_| CloudWorkerAuxiliaryError::InvalidDuration { field })?;
    let milliseconds = amount
        .checked_mul(multiplier)
        .ok_or(CloudWorkerAuxiliaryError::DurationOverflow { field })?;
    if milliseconds < minimum_ms {
        return Err(CloudWorkerAuxiliaryError::DurationTooShort { field });
    }
    Ok(milliseconds)
}

fn valid_environment_name(value: &str) -> bool {
    let mut characters = value.chars();
    let Some(first) = characters.next() else {
        return false;
    };
    (first.is_ascii_alphabetic() || first == '_')
        && characters.all(|character| character.is_ascii_alphanumeric() || character == '_')
}

fn valid_bounded_text(value: &str, max_chars: usize) -> bool {
    !value.trim().is_empty()
        && value.chars().count() <= max_chars
        && !value.chars().any(|character| character.is_control())
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum CloudWorkerAuxiliaryError {
    #[error("setup environment contains too many entries")]
    EnvironmentEntryLimitReached,
    #[error("setup environment name is empty, too long, or invalid")]
    InvalidEnvironmentName,
    #[error("setup environment value is unsafe or too long")]
    UnsafeEnvironmentValue,
    #[error("warm-image policy duration for {field} is invalid")]
    InvalidDuration { field: &'static str },
    #[error("warm-image policy duration for {field} overflows")]
    DurationOverflow { field: &'static str },
    #[error("warm-image policy duration for {field} is below the minimum")]
    DurationTooShort { field: &'static str },
    #[error("worker wallpaper is not a PNG with a valid IHDR header")]
    InvalidWallpaperPng,
    #[error("worker wallpaper dimensions are {width}x{height}, expected 1024x576")]
    InvalidWallpaperDimensions { width: u32, height: u32 },
    #[error("cloud-worker profile suggestion identity is invalid")]
    InvalidProfileSuggestion,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn environment_profile_is_sorted_and_rejects_shell_unsafe_values() {
        let mut entries = BTreeMap::new();
        entries.insert("Z_VAR".into(), "z".into());
        entries.insert("A_VAR".into(), "a".into());
        let profile = CloudWorkerEnvironmentProfile::new(entries).unwrap();
        assert_eq!(profile.names().collect::<Vec<_>>(), vec!["A_VAR", "Z_VAR"]);

        let mut unsafe_entries = BTreeMap::new();
        unsafe_entries.insert("SECRET".into(), "$(cat /secret)".into());
        assert_eq!(
            CloudWorkerEnvironmentProfile::new(unsafe_entries),
            Err(CloudWorkerAuxiliaryError::UnsafeEnvironmentValue)
        );

        let mut unsafe_name = BTreeMap::new();
        unsafe_name.insert("SECRET-NAME".into(), "value".into());
        assert_eq!(
            CloudWorkerEnvironmentProfile::new(unsafe_name),
            Err(CloudWorkerAuxiliaryError::InvalidEnvironmentName)
        );
    }

    #[test]
    fn warm_image_policy_matches_pinned_defaults_and_minimums() {
        assert_eq!(
            CloudWorkerWarmImagePolicy::from_config(None, None, false).unwrap(),
            CloudWorkerWarmImagePolicy {
                refresh_after_ms: 24 * 3_600_000,
                retain_unused_ms: 14 * 86_400_000,
                keep_previous: false,
            }
        );
        assert_eq!(
            CloudWorkerWarmImagePolicy::from_config(Some("30m"), None, false),
            Err(CloudWorkerAuxiliaryError::DurationTooShort {
                field: "refreshAfter"
            })
        );
    }

    #[test]
    fn warm_image_policy_rejects_zero_prefixed_and_unbounded_durations() {
        assert!(matches!(
            CloudWorkerWarmImagePolicy::from_config(Some("01h"), None, false),
            Err(CloudWorkerAuxiliaryError::InvalidDuration { .. })
        ));
        assert!(matches!(
            CloudWorkerWarmImagePolicy::from_config(Some("99999999d"), None, false),
            Ok(CloudWorkerWarmImagePolicy { .. })
        ));
        assert!(matches!(
            CloudWorkerWarmImagePolicy::from_config(Some("100000000d"), None, false),
            Err(CloudWorkerAuxiliaryError::InvalidDuration { .. })
        ));
    }

    #[test]
    fn wallpaper_contract_requires_the_pinned_png_dimensions() {
        let mut bytes = vec![0; 33];
        bytes[..8].copy_from_slice(PNG_SIGNATURE);
        bytes[8..12].copy_from_slice(&13u32.to_be_bytes());
        bytes[12..16].copy_from_slice(b"IHDR");
        bytes[16..20].copy_from_slice(&1024u32.to_be_bytes());
        bytes[20..24].copy_from_slice(&576u32.to_be_bytes());
        assert!(validate_worker_wallpaper_png(&bytes).is_ok());

        bytes[20..24].copy_from_slice(&575u32.to_be_bytes());
        assert_eq!(
            validate_worker_wallpaper_png(&bytes),
            Err(CloudWorkerAuxiliaryError::InvalidWallpaperDimensions {
                width: 1024,
                height: 575,
            })
        );
    }

    #[test]
    fn profile_suggestion_is_a_closed_setup_projection() {
        let suggestion = CloudWorkerProfileSuggestion::new("aws", "standard").unwrap();
        assert_eq!(suggestion.id, "cloud");
        assert_eq!(suggestion.provider, "crabbox");
        assert_eq!(suggestion.target, super::super::CloudWorkerOs::Linux);
        assert_eq!(suggestion.install, CloudWorkerInstallMode::Bundle);
        assert!(suggestion.warm_image);
        assert_eq!(
            CloudWorkerProfileSuggestion::new("aws\nunsafe", "standard"),
            Err(CloudWorkerAuxiliaryError::InvalidProfileSuggestion)
        );
    }
}
