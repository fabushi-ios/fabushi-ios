#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubagentModelForcePolicy {
    None,
    ParentPin,
    RequestBasedComposer,
}

impl SubagentModelForcePolicy {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::ParentPin => "parent_pin",
            Self::RequestBasedComposer => "request_based_composer",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn force_policy_wire_values_match_grok_reference() {
        assert_eq!(SubagentModelForcePolicy::None.as_str(), "none");
        assert_eq!(SubagentModelForcePolicy::ParentPin.as_str(), "parent_pin");
        assert_eq!(
            SubagentModelForcePolicy::RequestBasedComposer.as_str(),
            "request_based_composer"
        );
    }
}
