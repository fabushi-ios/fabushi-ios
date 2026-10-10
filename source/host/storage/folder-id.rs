pub fn is_safe_folder_id(id: &str) -> bool {
    !id.is_empty()
        && !id.contains('/')
        && !id.contains('\\')
        && !id.contains('\0')
        && id != "."
        && id != ".."
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_navigation_and_separator_ids() {
        for invalid in ["", ".", "..", "a/b", "a\\b", "a\0b"] {
            assert!(!is_safe_folder_id(invalid), "{invalid:?}");
        }
        assert!(is_safe_folder_id("agent-42"));
    }
}
