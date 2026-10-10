pub fn summarize_permission_request(title: &str, reason: &str) -> String {
    format!("Legacy permission request (no longer actionable): {title} — {reason}")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn preserves_legacy_non_actionable_summary() {
        assert_eq!(
            summarize_permission_request("Camera", "Need a photo"),
            "Legacy permission request (no longer actionable): Camera — Need a photo"
        );
    }
}
