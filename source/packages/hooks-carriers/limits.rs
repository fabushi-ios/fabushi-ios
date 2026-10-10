pub const HOOK_ADDITIONAL_CONTEXT_MAX_CHARS: usize = 10_000;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hook_additional_context_limit_matches_grok_reference() {
        assert_eq!(HOOK_ADDITIONAL_CONTEXT_MAX_CHARS, 10_000);
    }
}
