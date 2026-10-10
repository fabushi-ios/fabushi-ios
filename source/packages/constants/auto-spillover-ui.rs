#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AutoSpilloverUiDefaults {
    pub auto_title: &'static str,
    pub auto_description: &'static str,
    pub api_title: &'static str,
    pub api_description: &'static str,
    pub auto_beyond_limit_description: &'static str,
    pub auto_usage_bar_label: &'static str,
    pub api_usage_bar_label: &'static str,
}

pub const AUTO_SPILLOVER_UI_DEFAULTS: AutoSpilloverUiDefaults = AutoSpilloverUiDefaults {
    auto_title: "Cursor Models",
    auto_description: "Includes Cursor Grok 4.5 and Composer 2.5",
    api_title: "Other Models",
    api_description: "Consumed by named models.",
    auto_beyond_limit_description: "Additional usage beyond limits consumes Other Models quota or on-demand spend.",
    auto_usage_bar_label: "your included total usage",
    api_usage_bar_label: "your included API usage",
};

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn preserves_reference_copy() {
        assert_eq!(AUTO_SPILLOVER_UI_DEFAULTS.auto_title, "Cursor Models");
        assert_eq!(AUTO_SPILLOVER_UI_DEFAULTS.api_title, "Other Models");
        assert!(AUTO_SPILLOVER_UI_DEFAULTS.auto_description.contains("Composer 2.5"));
    }
}
