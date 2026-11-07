pub fn summarize_permission_request(title: &str, reason: &str) -> String {
    format!("Legacy permission request (no longer actionable): {title} — {reason}")
}
