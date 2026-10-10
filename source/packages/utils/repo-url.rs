pub fn is_origin_git_host(host: &str) -> bool {
    let lower = host.to_ascii_lowercase();
    let Some(prefix) = lower.strip_suffix(".cursor.com") else {
        return false;
    };
    if prefix == "origin" {
        return true;
    }
    let Some(suffix) = prefix.strip_prefix("origin-") else {
        return false;
    };
    !suffix.is_empty() && suffix.bytes().all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_only_reference_origin_hosts_case_insensitively() {
        for host in ["origin.cursor.com", "ORIGIN.CURSOR.COM", "origin-us1.cursor.com", "Origin-a9.Cursor.Com"] {
            assert!(is_origin_git_host(host), "{host}");
        }
        for host in ["cursor.com", "origin-.cursor.com", "origin_us.cursor.com", "origin-a.b.cursor.com", "evil-origin.cursor.com"] {
            assert!(!is_origin_git_host(host), "{host}");
        }
    }
}
