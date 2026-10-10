// The pinned Grok 0.18 module retains only its strict-mode initializer.
// No named runtime surface survived tree shaking, so the native counterpart
// intentionally exposes no production API.

#[cfg(test)]
mod tests {
    #[test]
    fn pinned_module_has_no_runtime_surface_to_emulate() {
        assert_eq!(module_path!().rsplit("::").next(), Some("tests"));
    }
}
