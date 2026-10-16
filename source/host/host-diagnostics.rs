use serde_json::{Map, Value};
use std::sync::{Arc, OnceLock, RwLock};

pub type HostDiagnostic = Map<String, Value>;
pub type HostDiagnosticsReporter = Arc<dyn Fn(&HostDiagnostic) + Send + Sync + 'static>;

fn reporter_slot() -> &'static RwLock<Option<HostDiagnosticsReporter>> {
    static REPORTER: OnceLock<RwLock<Option<HostDiagnosticsReporter>>> = OnceLock::new();
    REPORTER.get_or_init(|| RwLock::new(None))
}

pub fn pin_host_diagnostics_reporter(reporter: Option<HostDiagnosticsReporter>) {
    if let Ok(mut slot) = reporter_slot().write() {
        *slot = reporter;
    }
}

pub fn report_host_diagnostic(diagnostic: &HostDiagnostic) {
    let reporter = reporter_slot()
        .read()
        .ok()
        .and_then(|slot| slot.as_ref().cloned());
    if let Some(reporter) = reporter {
        reporter(diagnostic);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[test]
    fn pinned_reporter_receives_diagnostic() {
        let observed = Arc::new(Mutex::new(Vec::new()));
        let sink = observed.clone();
        pin_host_diagnostics_reporter(Some(Arc::new(move |diagnostic| {
            sink.lock().unwrap().push(diagnostic.get("kind").cloned());
        })));
        let mut diagnostic = HostDiagnostic::new();
        diagnostic.insert("kind".into(), Value::String("test".into()));
        report_host_diagnostic(&diagnostic);
        pin_host_diagnostics_reporter(None);
        assert_eq!(observed.lock().unwrap().len(), 1);
    }
}
