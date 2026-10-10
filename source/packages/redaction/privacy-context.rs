use crate::package_redaction_classification::DataClassification;
use crate::package_redaction_privacy_mode::PrivacyMode;

#[derive(Debug,Clone,Copy,PartialEq,Eq)]
pub struct PrivacyContext {
    pub privacy_mode: PrivacyMode,
    pub enforce_redaction: Option<bool>,
}

fn is_global_enforcement_enabled() -> bool {
    // The pinned 0.18 carrier declares the optional gate but exposes no setter;
    // absent a gate it resolves to false.
    false
}

pub fn resolve_enforce_redaction(
    context: PrivacyContext,
    classification: DataClassification,
) -> bool {
    match context.enforce_redaction {
        Some(false)=>false,
        Some(true)=>true,
        None=>{
            let _=classification;
            context.privacy_mode!=PrivacyMode::Unspecified && is_global_enforcement_enabled()
        }
    }
}

pub fn privacy_context_from_mode(
    privacy_mode: PrivacyMode,
    enforce_redaction: Option<bool>,
) -> PrivacyContext {
    PrivacyContext{privacy_mode,enforce_redaction}
}

pub fn to_privacy_context(mode: PrivacyMode) -> PrivacyContext {
    privacy_context_from_mode(mode,None)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn explicit_enforcement_overrides_default_gate() {
        let base=PrivacyContext{privacy_mode:PrivacyMode::NoStorage,enforce_redaction:None};
        assert!(!resolve_enforce_redaction(base,DataClassification::Credentials));
        assert!(resolve_enforce_redaction(PrivacyContext{enforce_redaction:Some(true),..base},DataClassification::Safe));
        assert!(!resolve_enforce_redaction(PrivacyContext{enforce_redaction:Some(false),..base},DataClassification::Credentials));
    }
    #[test]
    fn mode_conversion_preserves_optional_override() {
        assert_eq!(
            privacy_context_from_mode(PrivacyMode::NoTraining,Some(true)),
            PrivacyContext{privacy_mode:PrivacyMode::NoTraining,enforce_redaction:Some(true)}
        );
        assert_eq!(to_privacy_context(PrivacyMode::NoStorage).enforce_redaction,None);
    }
}
