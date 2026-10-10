pub struct SubagentComposerModelId;

impl SubagentComposerModelId {
    pub const STANDARD: &'static str = "composer-2.5";
    pub const FAST: &'static str = "composer-2.5-fast";
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn composer_model_ids_match_recovered_reference() {
        assert_eq!(SubagentComposerModelId::STANDARD, "composer-2.5");
        assert_eq!(SubagentComposerModelId::FAST, "composer-2.5-fast");
    }
}
