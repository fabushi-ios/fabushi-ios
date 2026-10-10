// The pinned Grok 0.18 module retains only its strict-mode initializer.
// No named runtime surface survived tree shaking, so the native counterpart
// intentionally exposes no production API.

#[cfg(test)]
mod tests {
    #[test]
    fn pinned_module_has_no_runtime_surface_to_emulate() {
        // Compiling this module is the parity contract: there is no callable
        // behavior in the pinned reference and no iOS-only behavior is invented.
        assert_eq!(module_path!().rsplit("::").next(), Some("tests"));
    }
}
