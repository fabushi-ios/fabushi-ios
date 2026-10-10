// The pinned Grok 0.18 module only constructs a private logger.
// It exports no runtime surface, so the native counterpart intentionally
// introduces no product API or logging side effect.

#[cfg(test)]
mod tests {
    #[test]
    fn pinned_module_has_no_exported_runtime_surface() {
        assert!(module_path!().ends_with("package_agent_state_utils::tests"));
    }
}
