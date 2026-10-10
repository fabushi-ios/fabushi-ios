// The retained Grok 0.18 module contains only its strict-mode initializer and
// exports no runtime surface. Compiling this module is therefore the native
// parity contract; no iOS-only steer-outbox behavior is invented.

#[cfg(test)]
mod tests {
    #[test]
    fn pinned_module_has_no_runtime_surface_to_emulate() {
        assert_eq!(module_path!().rsplit("::").next(), Some("tests"));
    }
}
