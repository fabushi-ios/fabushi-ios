use std::sync::{Arc, Mutex, OnceLock};

use serde_json::{Map, Value};

use crate::error_diagnostics::sanitize_error_diagnostic;

#[derive(Debug, Clone, PartialEq)]
pub struct HostDiagnostic {
    pub kind: String,
    pub fields: Map<String, Value>,
}

type Reporter = Arc<dyn Fn(&HostDiagnostic) + Send + Sync + 'static>;
static REPORTER: OnceLock<Mutex<Option<Reporter>>> = OnceLock::new();

fn slot() -> &'static Mutex<Option<Reporter>> {
    REPORTER.get_or_init(|| Mutex::new(None))
}

pub fn pin_host_diagnostics_reporter(reporter: Option<Reporter>) {
    *slot().lock().expect("host diagnostics reporter mutex poisoned") = reporter;
}

pub fn report_host_diagnostic(diagnostic: &HostDiagnostic) {
    let reporter = slot()
        .lock()
        .expect("host diagnostics reporter mutex poisoned")
        .clone();
    if let Some(reporter) = reporter {
        reporter(diagnostic);
    }
}

pub fn report_diagnostic_event(kind: impl Into<String>, fields: Value) {
    let fields_map = sanitize_diagnostic_fields(fields)
        .as_object()
        .cloned()
        .unwrap_or_default();
    report_host_diagnostic(&HostDiagnostic {
        kind: kind.into(),
        fields: fields_map,
    });
}

fn sanitize_diagnostic_fields(mut value: Value) -> Value {
    match &mut value {
        Value::String(text) => *text = sanitize_error_diagnostic(text),
        Value::Array(items) => {
            for item in items {
                let sanitized = sanitize_diagnostic_fields(std::mem::take(item));
                *item = sanitized;
            }
        }
        Value::Object(fields) => {
            for field in fields.values_mut() {
                let sanitized = sanitize_diagnostic_fields(std::mem::take(field));
                *field = sanitized;
            }
        }
        _ => {}
    }
    value
}

#[cfg(test)]
mod tests {
    use super::sanitize_diagnostic_fields;
    use serde_json::json;

    #[test]
    fn diagnostic_event_fields_redact_nested_credentials() {
        let sanitized = sanitize_diagnostic_fields(json!({
            "message": "Bearer secret-token",
            "nested": ["https://example.test/?token=secret", "Basic dXNlcjpwYXNz"],
        }));
        assert_eq!(sanitized["message"], "Bearer <redacted>");
        assert_eq!(sanitized["nested"][0], "https://example.test/?token=[REDACTED]");
        assert!(!sanitized["nested"][1].as_str().unwrap().contains("dXNlcjpwYXNz"));
    }
}
