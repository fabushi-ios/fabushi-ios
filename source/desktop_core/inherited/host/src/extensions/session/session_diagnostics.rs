use std::collections::BTreeMap;
use std::sync::{Arc, OnceLock, RwLock};

use serde_json::Value;

#[derive(Debug, Clone, PartialEq)]
pub struct SessionDiagnostic {
    pub family: String,
    pub kind: String,
    pub metadata: BTreeMap<String, Value>,
}

pub type SessionDiagnosticsReporter = Arc<dyn Fn(&SessionDiagnostic) + Send + Sync + 'static>;

fn reporter_slot() -> &'static RwLock<Option<SessionDiagnosticsReporter>> {
    static REPORTER: OnceLock<RwLock<Option<SessionDiagnosticsReporter>>> = OnceLock::new();
    REPORTER.get_or_init(|| RwLock::new(None))
}

pub fn pin_session_diagnostics_reporter(reporter: Option<SessionDiagnosticsReporter>) {
    if let Ok(mut slot) = reporter_slot().write() {
        *slot = reporter;
    }
}

pub fn report_session_diagnostic(report: &SessionDiagnostic) {
    let reporter = reporter_slot()
        .read()
        .ok()
        .and_then(|slot| slot.as_ref().map(Arc::clone));
    if let Some(reporter) = reporter {
        reporter(report);
    }
}
