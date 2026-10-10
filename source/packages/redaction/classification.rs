use crate::package_redaction_privacy_mode::PrivacyMode;

#[derive(Debug,Clone,Copy,PartialEq,Eq,Hash)]
pub enum DataClassification {
    Safe,
    Code,
    Credentials,
    Path,
    ProviderInfo,
    Unspecified,
}

pub const SENSITIVE_CLASSIFICATIONS: &[DataClassification] = &[
    DataClassification::Code,
    DataClassification::Credentials,
    DataClassification::Path,
    DataClassification::ProviderInfo,
    DataClassification::Unspecified,
];

#[derive(Debug,Clone,Copy,PartialEq,Eq,Hash)]
pub enum PrivacyCapability {
    StorageForTraining,
    StorageForLogging,
    StorageForUsage,
    UnsafeAlwaysAllowed,
}

pub fn allowed_purpose(
    privacy_mode: PrivacyMode,
    purpose: PrivacyCapability,
    classification: DataClassification,
) -> bool {
    if classification==DataClassification::Safe { return true; }
    if purpose==PrivacyCapability::UnsafeAlwaysAllowed { return true; }
    if matches!(classification,DataClassification::Credentials|DataClassification::Unspecified) {
        return false;
    }
    match privacy_mode {
        PrivacyMode::NoStorage|PrivacyMode::Unspecified=>false,
        PrivacyMode::NoTraining=>match purpose {
            PrivacyCapability::StorageForLogging=>matches!(classification,DataClassification::Path|DataClassification::ProviderInfo),
            PrivacyCapability::StorageForTraining=>false,
            PrivacyCapability::StorageForUsage=>true,
            PrivacyCapability::UnsafeAlwaysAllowed=>true,
        },
        PrivacyMode::UsageDataTrainingAllowed|PrivacyMode::UsageCodebaseTrainingAllowed=>true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn safe_and_unsafe_always_allowed_short_circuit() {
        for mode in [
            PrivacyMode::Unspecified,PrivacyMode::NoStorage,PrivacyMode::NoTraining,
            PrivacyMode::UsageDataTrainingAllowed,PrivacyMode::UsageCodebaseTrainingAllowed
        ] {
            assert!(allowed_purpose(mode,PrivacyCapability::StorageForTraining,DataClassification::Safe));
            assert!(allowed_purpose(mode,PrivacyCapability::UnsafeAlwaysAllowed,DataClassification::Credentials));
        }
    }
    #[test]
    fn credentials_and_unspecified_are_denied_except_unsafe_override() {
        assert!(!allowed_purpose(PrivacyMode::UsageCodebaseTrainingAllowed,PrivacyCapability::StorageForUsage,DataClassification::Credentials));
        assert!(!allowed_purpose(PrivacyMode::UsageDataTrainingAllowed,PrivacyCapability::StorageForLogging,DataClassification::Unspecified));
    }
    #[test]
    fn no_training_preserves_logging_and_usage_matrix() {
        assert!(allowed_purpose(PrivacyMode::NoTraining,PrivacyCapability::StorageForLogging,DataClassification::Path));
        assert!(allowed_purpose(PrivacyMode::NoTraining,PrivacyCapability::StorageForLogging,DataClassification::ProviderInfo));
        assert!(!allowed_purpose(PrivacyMode::NoTraining,PrivacyCapability::StorageForLogging,DataClassification::Code));
        assert!(!allowed_purpose(PrivacyMode::NoTraining,PrivacyCapability::StorageForTraining,DataClassification::Code));
        assert!(allowed_purpose(PrivacyMode::NoTraining,PrivacyCapability::StorageForUsage,DataClassification::Code));
    }
}
