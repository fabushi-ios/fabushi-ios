use url::Url;

pub fn sanitize_remote_url_for_prompt(remote_url: &str) -> String {
    let trimmed = remote_url.trim();
    if trimmed.is_empty() {
        return String::new();
    }

    let Ok(mut parsed) = Url::parse(trimmed) else {
        return trimmed.to_owned();
    };

    let _ = parsed.set_username("");
    let _ = parsed.set_password(None);
    parsed.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn removes_url_credentials_and_preserves_invalid_text() {
        assert_eq!(
            sanitize_remote_url_for_prompt("  https://user:secret@example.com/path?q=1  "),
            "https://example.com/path?q=1"
        );
        assert_eq!(
            sanitize_remote_url_for_prompt("ssh://alice:token@example.com/repo"),
            "ssh://example.com/repo"
        );
        assert_eq!(sanitize_remote_url_for_prompt("  not a url  "), "not a url");
        assert_eq!(sanitize_remote_url_for_prompt("   "), "");
    }
}
