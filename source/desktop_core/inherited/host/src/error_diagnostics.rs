pub const MAX_ERROR_DIAGNOSTIC_CHARS: usize = 2048;

use crate::extensions::telemetry::openclaw_secret_redactor::{
    redact_auth_header, redact_pem_blocks, redact_sensitive_query_params,
};

pub fn sanitize_error_diagnostic(raw: &str) -> String {
    let raw = redact_sensitive_query_params(&redact_pem_blocks(raw));
    let mut sanitized = String::with_capacity(raw.len());
    let mut words = raw.split_whitespace().peekable();
    while let Some(word) = words.next() {
        if !sanitized.is_empty() {
            sanitized.push(' ');
        }
        if word.eq_ignore_ascii_case("bearer") {
            sanitized.push_str("Bearer <redacted>");
            let _ = words.next();
            continue;
        }
        if word.eq_ignore_ascii_case("basic") {
            let credential = words.next().unwrap_or_default();
            sanitized.push_str(&redact_auth_header(&format!("Basic {credential}")));
            continue;
        }
        if word.starts_with("sk-") || word.starts_with("ghp_") || word.starts_with("xoxb-") {
            sanitized.push_str("<redacted-secret>");
            continue;
        }
        if word.starts_with("password=") || word.starts_with("token=") || word.starts_with("secret=") {
            let key = word.split('=').next().unwrap_or("secret");
            sanitized.push_str(&format!("{key}=<redacted>"));
            continue;
        }
        sanitized.push_str(word);
    }

    let mut chars: Vec<char> = sanitized.chars().collect();
    if chars.len() > MAX_ERROR_DIAGNOSTIC_CHARS {
        chars.truncate(MAX_ERROR_DIAGNOSTIC_CHARS);
    }
    chars.into_iter().collect::<String>().trim().to_string()
}

pub fn format_error_message_for_display(message: &str, diagnostic: Option<&str>) -> String {
    match diagnostic.map(str::trim).filter(|d| !d.is_empty()) {
        Some(diag) => format!("{message}\n{diag}"),
        None => message.to_string(),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiagnosticError {
    pub message: String,
    pub diagnostic: Option<String>,
}

impl DiagnosticError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            diagnostic: None,
        }
    }

    pub fn with_diagnostic(mut self, diagnostic: impl AsRef<str>) -> Self {
        self.diagnostic = Some(sanitize_error_diagnostic(diagnostic.as_ref()));
        self
    }

    pub fn format_display(&self) -> String {
        format_error_message_for_display(&self.message, self.diagnostic.as_deref())
    }
}
