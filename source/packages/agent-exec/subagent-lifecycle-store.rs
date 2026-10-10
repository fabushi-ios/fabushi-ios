// The pinned Grok 0.18 module preserves only the evaluation edge to
// subagent-lifecycle-state-machine. That dependency has no retained runtime
// surface, so the Rust counterpart intentionally adds no production API.

#[cfg(test)]
mod tests {
    #[test]
    fn retained_store_module_has_no_runtime_surface_to_emulate() {
        assert!(module_path!().ends_with("package_agent_exec_subagent_lifecycle_store::tests"));
    }
}
