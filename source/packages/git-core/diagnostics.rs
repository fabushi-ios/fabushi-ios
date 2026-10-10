use crate::package_git_core_redaction::redact_http_url_userinfo;

pub const GIT_STDERR_LOG_CAP: usize = 2_048;

#[derive(Debug,Default,Clone,PartialEq)]
pub struct GitDiagnosticError {
    pub git_error_code: Option<String>,
    pub exit_code: Option<f64>,
    pub stderr: Option<String>,
}

fn cap_stderr(stderr:&str)->String {
    if stderr.chars().count()<=GIT_STDERR_LOG_CAP { return stderr.to_owned(); }
    let prefix=stderr.chars().take(GIT_STDERR_LOG_CAP).collect::<String>();
    format!("{prefix}… [truncated to {GIT_STDERR_LOG_CAP} chars]")
}

pub fn append_git_diagnostics(
    message:&str,
    error:Option<&GitDiagnosticError>,
    include_stderr:bool,
)->String {
    if message.contains(" [git: ") && message[message.find(" [git: ").unwrap_or(0)..].contains(']') {
        return message.to_owned();
    }
    let Some(error)=error else { return message.to_owned(); };
    let mut parts=Vec::new();
    if let Some(code)=error.git_error_code.as_deref().filter(|value|!value.is_empty()) {
        parts.push(format!("gitErrorCode={code}"));
    }
    if let Some(exit_code)=error.exit_code.filter(|value|value.is_finite()) {
        let rendered=if exit_code.fract()==0.0 { format!("{}",exit_code as i64) } else { exit_code.to_string() };
        parts.push(format!("exitCode={rendered}"));
    }
    if parts.is_empty(){return message.to_owned();}
    let mut annotated=format!("{message} [git: {}]",parts.join(" "));
    if include_stderr {
        if let Some(stderr)=error.stderr.as_deref().filter(|value|!value.is_empty()) {
            annotated.push_str("\n--- git stderr ---\n");
            annotated.push_str(&cap_stderr(&redact_http_url_userinfo(stderr)));
        }
    }
    annotated
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn appends_codes_once_and_optionally_redacts_stderr() {
        let error=GitDiagnosticError{
            git_error_code:Some("AUTH".into()),
            exit_code:Some(128.0),
            stderr:Some("fatal https://user:pass@example.com/repo.git?token=x".into()),
        };
        let message=append_git_diagnostics("clone failed",Some(&error),true);
        assert!(message.starts_with("clone failed [git: gitErrorCode=AUTH exitCode=128]"));
        assert!(message.contains("https://example.com/repo.git"));
        assert!(!message.contains("user:pass"));
        assert!(!message.contains("token=x"));
        assert_eq!(append_git_diagnostics(&message,Some(&error),true),message);
    }

    #[test]
    fn ignores_errors_without_supported_diagnostic_fields() {
        let error=GitDiagnosticError{stderr:Some("details".into()),..Default::default()};
        assert_eq!(append_git_diagnostics("failed",Some(&error),true),"failed");
    }

    #[test]
    fn caps_stderr_at_reference_limit() {
        let error=GitDiagnosticError{
            git_error_code:Some("X".into()),
            stderr:Some("a".repeat(3_000)),
            ..Default::default()
        };
        let message=append_git_diagnostics("failed",Some(&error),true);
        assert!(message.contains("… [truncated to 2048 chars]"));
    }
}
