use std::collections::{HashMap, HashSet};

use crate::extensions::telemetry::openclaw_secret_redactor::redact_sensitive_query_params;

#[derive(Debug, Clone)]
pub struct ProxyHeaderRedactor {
    sensitive_headers: HashSet<String>,
}

impl Default for ProxyHeaderRedactor {
    fn default() -> Self {
        let mut set = HashSet::new();
        for h in &[
            "authorization",
            "proxy-authorization",
            "x-api-key",
            "cookie",
            "set-cookie",
            "proxy-authenticate",
            "x-auth-token",
        ] {
            set.insert(h.to_string());
        }
        Self {
            sensitive_headers: set,
        }
    }
}

impl ProxyHeaderRedactor {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn redact_headers(&self, headers: &HashMap<String, String>) -> HashMap<String, String> {
        self.redact_headers_with_additional(headers, std::iter::empty::<&str>())
    }

    pub fn redact_headers_with_additional<'a, I>(
        &self,
        headers: &HashMap<String, String>,
        additional_sensitive_names: I,
    ) -> HashMap<String, String>
    where
        I: IntoIterator<Item = &'a str>,
    {
        let additional_sensitive = additional_sensitive_names
            .into_iter()
            .map(|name| name.trim().to_ascii_lowercase())
            .filter(|name| !name.is_empty())
            .collect::<HashSet<_>>();
        let mut result = HashMap::new();
        for (k, v) in headers {
            let normalized = k.trim().to_ascii_lowercase();
            if additional_sensitive.contains(&normalized) || self.is_sensitive_header(&normalized) {
                result.insert(k.clone(), "[REDACTED]".to_string());
            } else {
                result.insert(k.clone(), v.clone());
            }
        }
        result
    }

    pub fn is_sensitive_header(&self, header_name: &str) -> bool {
        let normalized = header_name.trim().to_ascii_lowercase();
        self.sensitive_headers.contains(&normalized)
            || normalized.contains("api-key")
            || normalized.contains("apikey")
            || normalized.contains("token")
            || normalized.contains("secret")
            || normalized.contains("password")
            || normalized.contains("credential")
            || normalized.contains("session")
    }

    pub fn redact_url(&self, url: &str) -> String {
        redact_sensitive_query_params(url)
    }
}
