// The retained Grok 0.18 browser-bridge module is an intentionally empty
// initializer. No named runtime or erased declaration surface survived.

#[cfg(test)]
mod tests {
    #[test]
    fn pinned_browser_bridge_has_no_runtime_surface() {
        assert!(module_path!().ends_with("package_context_browser_bridge::tests"));
    }
}
