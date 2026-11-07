//! Server-Side Request Forgery (SSRF) Policy & Protection
//!
//! Mirrors:
//! - `src/infra/net/ssrf.ts`
//! - `src/plugin-sdk/ssrf-policy.ts`
//! - `src/plugin-sdk/ssrf-runtime.ts`

use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::net::IpAddr;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SsrFPolicy {
    pub allow_private_network: bool,
    pub allowed_schemes: Vec<String>,
    pub blocked_hosts: HashSet<String>,
    pub allowed_hosts: HashSet<String>,
}

pub type SsrfPolicy = SsrFPolicy;

impl Default for SsrFPolicy {
    fn default() -> Self {
        Self {
            allow_private_network: false,
            allowed_schemes: vec!["http".to_string(), "https".to_string()],
            blocked_hosts: HashSet::new(),
            allowed_hosts: HashSet::new(),
        }
    }
}

impl SsrFPolicy {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn allow_private_network(mut self, allow: bool) -> Self {
        self.allow_private_network = allow;
        self
    }

    pub fn allow_host(mut self, host: &str) -> Self {
        self.allowed_hosts.insert(host.trim().to_ascii_lowercase());
        self
    }

    pub fn block_host(mut self, host: &str) -> Self {
        self.blocked_hosts.insert(host.trim().to_ascii_lowercase());
        self
    }

    pub fn validate_url(&self, url_str: &str) -> Result<url::Url, String> {
        let parsed = match url::Url::parse(url_str) {
            Ok(u) => u,
            Err(e) => return Err(format!("Invalid URL syntax: {e}")),
        };

        let scheme = parsed.scheme().to_ascii_lowercase();
        if !self.allowed_schemes.contains(&scheme) {
            return Err(format!(
                "SSRF violation: scheme '{scheme}' is not allowed (allowed: {:?})",
                self.allowed_schemes
            ));
        }

        let host = match parsed.host_str() {
            Some(h) => h.to_ascii_lowercase(),
            None => return Err("SSRF violation: missing host component".to_string()),
        };

        // Explicit allowlist bypasses further checks
        if self.allowed_hosts.contains(&host) {
            return Ok(parsed);
        }

        // Explicit blocked hosts
        if self.blocked_hosts.contains(&host) {
            return Err(format!("SSRF violation: host '{host}' is explicitly blocked"));
        }

        // Loopback hostnames
        if host == "localhost"
            || host.ends_with(".localhost")
            || host == "local"
            || host.ends_with(".local")
            || host == "127.0.0.1"
            || host == "::1"
            || host == "0.0.0.0"
        {
            if !self.allow_private_network {
                return Err(format!("SSRF violation: loopback host '{host}' is prohibited"));
            }
        }

        // Cloud metadata hostname
        if host == "metadata.google.internal" || host == "169.254.169.254" {
            return Err("SSRF violation: cloud instance metadata endpoint is prohibited".to_string());
        }

        // IP address checks
        if let Ok(ip) = host.parse::<IpAddr>() {
            match ip {
                IpAddr::V4(ipv4) => {
                    let octets = ipv4.octets();
                    // Loopback: 127.0.0.0/8
                    if octets[0] == 127 && !self.allow_private_network {
                        return Err(format!("SSRF violation: loopback IPv4 '{ip}' is prohibited"));
                    }
                    // Private RFC 1918: 10.0.0.0/8, 172.16.0.0/12, 192.168.0.0/16
                    let is_private = octets[0] == 10
                        || (octets[0] == 172 && (16..=31).contains(&octets[1]))
                        || (octets[0] == 192 && octets[1] == 168);
                    if is_private && !self.allow_private_network {
                        return Err(format!("SSRF violation: private RFC1918 IPv4 '{ip}' is prohibited"));
                    }
                    // Link-local / Cloud metadata: 169.254.0.0/16
                    if octets[0] == 169 && octets[1] == 254 {
                        return Err(format!("SSRF violation: link-local/cloud-metadata IPv4 '{ip}' is prohibited"));
                    }
                    // Unspecified: 0.0.0.0/8
                    if octets[0] == 0 && !self.allow_private_network {
                        return Err(format!("SSRF violation: unspecified IPv4 '{ip}' is prohibited"));
                    }
                }
                IpAddr::V6(ipv6) => {
                    if (ipv6.is_loopback() || ipv6.is_unspecified()) && !self.allow_private_network {
                        return Err(format!("SSRF violation: loopback/unspecified IPv6 '{ip}' is prohibited"));
                    }
                }
            }
        }

        Ok(parsed)
    }
}
