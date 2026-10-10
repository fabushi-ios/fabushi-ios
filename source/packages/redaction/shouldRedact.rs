use crate::package_redaction_classification::{DataClassification,SENSITIVE_CLASSIFICATIONS};
use crate::package_redaction_privacy_context::{PrivacyContext,resolve_enforce_redaction};
use crate::package_redaction_privacy_mode::PrivacyMode;

pub fn should_redact(privacy_mode:PrivacyMode,classification:DataClassification)->bool {
    if matches!(classification,DataClassification::Credentials|DataClassification::Unspecified){return true;}
    if classification==DataClassification::Safe{return false;}
    if !SENSITIVE_CLASSIFICATIONS.contains(&classification){return false;}
    match privacy_mode {
        PrivacyMode::UsageDataTrainingAllowed|PrivacyMode::UsageCodebaseTrainingAllowed=>false,
        PrivacyMode::NoStorage|PrivacyMode::NoTraining|PrivacyMode::Unspecified=>true,
    }
}

pub fn format_redacted(field_name:&str)->String {
    format!("[redacted:{field_name}]")
}

#[derive(Debug,Clone,Copy)]
pub struct RedactionDisplayOptions<'a> {
    pub privacy_mode:PrivacyMode,
    pub classification:DataClassification,
    pub field_name:&'a str,
    pub unredacted_value:&'a str,
    pub enforce_redaction:Option<bool>,
}

pub fn get_redaction_aware_display_value(options:RedactionDisplayOptions<'_>)->String {
    let enforce=resolve_enforce_redaction(
        PrivacyContext{privacy_mode:options.privacy_mode,enforce_redaction:options.enforce_redaction},
        options.classification
    );
    if !enforce{return options.unredacted_value.to_owned();}
    if should_redact(options.privacy_mode,options.classification) {
        format_redacted(options.field_name)
    } else {
        options.unredacted_value.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn credentials_and_unspecified_are_always_policy_redacted() {
        for mode in [
            PrivacyMode::Unspecified,PrivacyMode::NoStorage,PrivacyMode::NoTraining,
            PrivacyMode::UsageDataTrainingAllowed,PrivacyMode::UsageCodebaseTrainingAllowed
        ] {
            assert!(should_redact(mode,DataClassification::Credentials));
            assert!(should_redact(mode,DataClassification::Unspecified));
        }
    }
    #[test]
    fn sensitive_noncredential_data_follows_mode() {
        assert!(should_redact(PrivacyMode::NoStorage,DataClassification::Code));
        assert!(should_redact(PrivacyMode::NoTraining,DataClassification::Path));
        assert!(!should_redact(PrivacyMode::UsageDataTrainingAllowed,DataClassification::ProviderInfo));
        assert!(!should_redact(PrivacyMode::UsageCodebaseTrainingAllowed,DataClassification::Code));
        assert!(!should_redact(PrivacyMode::NoStorage,DataClassification::Safe));
    }
    #[test]
    fn display_redacts_only_when_enforcement_resolves_true() {
        let opts=RedactionDisplayOptions{
            privacy_mode:PrivacyMode::NoStorage,
            classification:DataClassification::Credentials,
            field_name:"token",
            unredacted_value:"secret",
            enforce_redaction:None,
        };
        assert_eq!(get_redaction_aware_display_value(opts),"secret");
        assert_eq!(
            get_redaction_aware_display_value(RedactionDisplayOptions{enforce_redaction:Some(true),..opts}),
            "[redacted:token]"
        );
        assert_eq!(
            get_redaction_aware_display_value(RedactionDisplayOptions{classification:DataClassification::Safe,enforce_redaction:Some(true),..opts}),
            "secret"
        );
    }
}
